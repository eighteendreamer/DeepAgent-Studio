//! The stdio MCP transport: spawn a local server process and speak
//! newline-delimited JSON-RPC 2.0 over its stdin/stdout.
//!
//! This is the most common MCP transport (npx-packaged servers, custom local
//! servers). A single reader dispatches stdout responses to pending requests by
//! id. Request waiters never own the reader. The child is terminated on
//! [`McpTransport::close`].

use std::collections::{HashMap, VecDeque};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use async_trait::async_trait;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio::sync::{oneshot, Mutex};

use deepagent_core::error::{CoreError, Result};
use deepagent_core::redaction::{is_secret_key, scrub_secret_literals};
use deepagent_tools::ToolExecutionContext;

use crate::config::McpServerConfig;
use crate::protocol::{JsonRpcNotification, JsonRpcRequest, JsonRpcResponse};
use crate::transport::{
    McpTransport, MCP_CANCELLED_BEFORE_DISPATCH, MCP_CANCELLED_UNKNOWN_OUTCOME,
};

#[cfg(windows)]
#[path = "stdio_windows_job.rs"]
mod windows_job;
const STDERR_LIMIT: usize = 8 * 1024;
const MAX_PENDING_REQUESTS: usize = 1024;

// Runtime base only, not a heuristic over secret-looking names. Other variables
// (including proxy/CA configuration) require an explicit config.env entry.
// Adapted to this host from Codex's local MCP base environment; this is not an
// OS sandbox and does not restrict access to files belonging to the same user.
#[cfg(windows)]
const STDIO_BASE_ENV: &[&str] = &[
    "PATH",
    "PATHEXT",
    "SHELL",
    "COMSPEC",
    "SYSTEMROOT",
    "WINDIR",
    "SYSTEMDRIVE",
    "USERNAME",
    "USERDOMAIN",
    "USERPROFILE",
    "HOMEDRIVE",
    "HOMEPATH",
    "PROGRAMFILES",
    "PROGRAMFILES(X86)",
    "PROGRAMW6432",
    "PROGRAMDATA",
    "LOCALAPPDATA",
    "APPDATA",
    "TEMP",
    "TMP",
    "TMPDIR",
    "HOME",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "TERM",
    "TZ",
];
#[cfg(not(windows))]
const STDIO_BASE_ENV: &[&str] = &[
    "HOME",
    "LOGNAME",
    "PATH",
    "SHELL",
    "USER",
    "__CF_USER_TEXT_ENCODING",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "TERM",
    "TMPDIR",
    "TEMP",
    "TMP",
    "TZ",
];

type ResponseSender = oneshot::Sender<std::result::Result<JsonRpcResponse, String>>;

#[derive(Default)]
struct ResponseDispatcher {
    state: StdMutex<DispatchState>,
}

#[derive(Default)]
struct DispatchState {
    pending: HashMap<u64, PendingResponse>,
    terminal_error: Option<String>,
}

struct PendingResponse {
    sender: ResponseSender,
    identity: Arc<()>,
}

impl ResponseDispatcher {
    fn register(&self, id: u64, sender: ResponseSender) -> Result<PendingRequest<'_>> {
        let mut state = self.state.lock().expect("MCP dispatcher poisoned");
        if let Some(reason) = &state.terminal_error {
            return Err(CoreError::other(reason));
        }
        if state.pending.contains_key(&id) {
            return Err(CoreError::invalid("duplicate pending MCP request id"));
        }
        if state.pending.len() >= MAX_PENDING_REQUESTS {
            return Err(CoreError::invalid("MCP pending request limit exceeded"));
        }
        let identity = Arc::new(());
        state.pending.insert(
            id,
            PendingResponse {
                sender,
                identity: identity.clone(),
            },
        );
        Ok(PendingRequest {
            dispatcher: self,
            id,
            identity,
        })
    }

    fn check_open(&self) -> Result<()> {
        let state = self.state.lock().expect("MCP dispatcher poisoned");
        match &state.terminal_error {
            Some(reason) => Err(CoreError::other(reason)),
            None => Ok(()),
        }
    }

    fn deliver(&self, response: JsonRpcResponse) {
        let sender = self
            .state
            .lock()
            .expect("MCP dispatcher poisoned")
            .pending
            .remove(&response.id);
        if let Some(pending) = sender {
            let _ = pending.sender.send(Ok(response));
        }
        // Unknown and late ids are discarded, never buffered for a future request.
    }

    fn finish(&self, reason: &str) {
        let pending = {
            let mut state = self.state.lock().expect("MCP dispatcher poisoned");
            if state.terminal_error.is_some() {
                return;
            }
            state.terminal_error = Some(reason.to_string());
            std::mem::take(&mut state.pending)
        };
        for (_, pending) in pending {
            let _ = pending.sender.send(Err(reason.to_string()));
        }
    }
}

/// Removing a waiter must also happen when its future is dropped by a caller's
/// timeout/abort, without spawning cleanup or cancelling other requests.
struct PendingRequest<'a> {
    dispatcher: &'a ResponseDispatcher,
    id: u64,
    identity: Arc<()>,
}

impl Drop for PendingRequest<'_> {
    fn drop(&mut self) {
        let mut state = self
            .dispatcher
            .state
            .lock()
            .expect("MCP dispatcher poisoned");
        if state
            .pending
            .get(&self.id)
            .is_some_and(|pending| Arc::ptr_eq(&pending.identity, &self.identity))
        {
            state.pending.remove(&self.id);
        }
    }
}

/// Invocation-local evidence for cancellation, not a peer execution guarantee.
#[derive(Clone, Copy, PartialEq, Eq)]
enum MessageWriteState {
    Queued,
    Writing,
    Flushed,
}

/// A partially written JSON line cannot be resumed by another request. Retire
/// this connection if a write future is dropped before write+flush completes.
struct MessageWrite<'a> {
    dispatcher: &'a ResponseDispatcher,
    state: &'a mut MessageWriteState,
}

impl Drop for MessageWrite<'_> {
    fn drop(&mut self) {
        if *self.state == MessageWriteState::Writing {
            self.dispatcher
                .finish("MCP connection lost during message write");
        }
    }
}

async fn dispatch_responses(stdout: ChildStdout, dispatcher: Arc<ResponseDispatcher>) {
    let mut reader = BufReader::new(stdout);
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line).await {
            Ok(0) => {
                dispatcher.finish("MCP server closed the connection");
                return;
            }
            Err(error) => {
                dispatcher.finish(&format!("read from MCP server failed: {error}"));
                return;
            }
            Ok(_) => {
                if let Ok(response) = serde_json::from_str::<JsonRpcResponse>(line.trim()) {
                    dispatcher.deliver(response);
                }
            }
        }
    }
}

/// A stdio transport backed by a spawned child process.
pub struct StdioTransport {
    child: Mutex<Child>,
    #[cfg(windows)]
    process_job: windows_job::ProcessJob,
    stdin: Mutex<ChildStdin>,
    dispatcher: Arc<ResponseDispatcher>,
    reader_task: tokio::task::JoinHandle<()>,
    next_wire_id: AtomicU64,
    stderr: Arc<StdMutex<VecDeque<u8>>>,
    stderr_task: Mutex<tokio::task::JoinHandle<()>>,
    secrets: Vec<String>,
}

impl StdioTransport {
    /// Spawn the server described by `config` (must be a stdio config).
    pub fn spawn(config: &McpServerConfig) -> Result<Self> {
        let command = config
            .command
            .as_deref()
            .ok_or_else(|| CoreError::invalid("stdio transport requires 'command'"))?;

        let mut cmd = tokio::process::Command::new(command);
        cmd.args(&config.args)
            .env_clear()
            .envs(
                STDIO_BASE_ENV
                    .iter()
                    .filter_map(|name| std::env::var_os(name).map(|value| (*name, value))),
            )
            .envs(&config.env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        tracing::debug!(
            policy = "runtime_base_and_config",
            explicit_variables = config.env.len(),
            "MCP stdio environment policy applied"
        );
        if let Some(cwd) = &config.cwd {
            cmd.current_dir(cwd);
        }
        #[cfg(windows)]
        let (mut child, process_job) = windows_job::ProcessJob::spawn(&mut cmd).map_err(|e| {
            CoreError::other(format!(
                "failed to spawn contained MCP server '{command}': {e}"
            ))
        })?;
        #[cfg(not(windows))]
        let mut child = cmd.spawn().map_err(|e| {
            CoreError::other(format!("failed to spawn MCP server '{command}': {e}"))
        })?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| CoreError::other("child stdin unavailable"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| CoreError::other("child stdout unavailable"))?;
        let mut stderr_reader = child
            .stderr
            .take()
            .ok_or_else(|| CoreError::other("child stderr unavailable"))?;
        let stderr = Arc::new(StdMutex::new(VecDeque::new()));
        let captured = stderr.clone();
        // Drain continuously: a verbose server must not block on a full pipe.
        let stderr_task = tokio::spawn(async move {
            let mut chunk = [0u8; 2048];
            loop {
                match stderr_reader.read(&mut chunk).await {
                    Ok(0) | Err(_) => break,
                    Ok(count) => {
                        if let Ok(mut tail) = captured.lock() {
                            tail.extend(&chunk[..count]);
                            let excess = tail.len().saturating_sub(STDERR_LIMIT);
                            tail.drain(..excess);
                        }
                    }
                }
            }
        });
        let mut secrets: Vec<String> = std::env::vars()
            .chain(config.env.clone())
            .filter(|(key, value)| is_secret_key(key) && !value.is_empty())
            .map(|(_, value)| value)
            .collect();
        secrets.sort_by_key(|value| std::cmp::Reverse(value.len()));
        secrets.dedup();

        let dispatcher = Arc::new(ResponseDispatcher::default());
        let reader_task = tokio::spawn(dispatch_responses(stdout, dispatcher.clone()));
        Ok(Self {
            child: Mutex::new(child),
            #[cfg(windows)]
            process_job,
            stdin: Mutex::new(stdin),
            dispatcher,
            reader_task,
            next_wire_id: AtomicU64::new(1),
            stderr,
            stderr_task: Mutex::new(stderr_task),
            secrets,
        })
    }

    async fn disconnect_error(&self, reason: &str) -> CoreError {
        let mut child = self.child.lock().await;
        let status = match child.try_wait() {
            Ok(Some(status)) => Some(status),
            Ok(None) => tokio::time::timeout(Duration::from_millis(200), child.wait())
                .await
                .ok()
                .and_then(|r| r.ok()),
            Err(_) => None,
        };
        drop(child);
        // Give the draining task a bounded chance to consume the final error.
        let mut task = self.stderr_task.lock().await;
        if !task.is_finished() {
            let _ = tokio::time::timeout(Duration::from_millis(200), &mut *task).await;
        }
        let bytes = self
            .stderr
            .lock()
            .map(|tail| tail.iter().copied().collect::<Vec<_>>())
            .unwrap_or_default();
        // A truncated head could start halfway through a credential. Discard
        // that fragment before redaction rather than exposing a token suffix.
        let bytes = if bytes.len() == STDERR_LIMIT {
            bytes
                .iter()
                .position(|byte| *byte == b'\n')
                .map(|pos| &bytes[pos + 1..])
                .unwrap_or(&[])
        } else {
            &bytes
        };
        let mut diagnostic = String::from_utf8_lossy(bytes).into_owned();
        for secret in &self.secrets {
            diagnostic = diagnostic.replace(secret, "<redacted>");
        }
        let diagnostic = scrub_secret_literals(diagnostic.trim());
        let status = status.map(|s| format!(" ({s})")).unwrap_or_default();
        CoreError::other(if diagnostic.is_empty() {
            format!("{reason}{status}")
        } else {
            format!("{reason}{status}; stderr: {diagnostic}")
        })
    }

    async fn send_cancelled_notification(&self, request_id: u64, reason: &str) -> Result<()> {
        self.write_notification(
            "notifications/cancelled",
            Some(serde_json::json!({ "requestId": request_id, "reason": reason })),
        )
        .await
    }

    async fn notify_cancelled_bounded(&self, request_id: u64) {
        match tokio::time::timeout(
            Duration::from_millis(50),
            self.send_cancelled_notification(request_id, "user requested cancellation"),
        )
        .await
        {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                tracing::debug!(%error, request_id, "could not send MCP cancellation notification")
            }
            Err(_) => tracing::debug!(request_id, "MCP cancellation notification write timed out"),
        }
    }

    async fn write_message<T: serde::Serialize + Sync>(
        &self,
        message: &T,
        state: &mut MessageWriteState,
    ) -> Result<()> {
        let mut line = serde_json::to_string(message)?;
        line.push('\n');
        let mut stdin = self.stdin.lock().await;
        self.dispatcher.check_open()?;
        *state = MessageWriteState::Writing;
        let write = MessageWrite {
            dispatcher: &self.dispatcher,
            state,
        };
        if let Err(error) = stdin.write_all(line.as_bytes()).await {
            self.dispatcher
                .finish("write to MCP server failed; connection lost");
            return Err(self
                .disconnect_error(&format!("write to MCP server failed: {error}"))
                .await);
        }
        if let Err(error) = stdin.flush().await {
            self.dispatcher
                .finish("flush to MCP server failed; connection lost");
            return Err(self
                .disconnect_error(&format!("flush to MCP server failed: {error}"))
                .await);
        }
        *write.state = MessageWriteState::Flushed;
        Ok(())
    }

    async fn write_notification(
        &self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<()> {
        self.write_message(
            &JsonRpcNotification::new(method, params),
            &mut MessageWriteState::Queued,
        )
        .await
    }

    async fn send_request(
        &self,
        request: &JsonRpcRequest,
        context: ToolExecutionContext,
    ) -> Result<JsonRpcResponse> {
        if context.is_cancelled() {
            return Err(CoreError::other(MCP_CANCELLED_BEFORE_DISPATCH));
        }
        // Different clients (including reconnect initialization) can share a
        // transport. Allocate ids per connection so a late response cannot
        // satisfy a new caller that reused a client-local id.
        let wire_id = self
            .next_wire_id
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
            .map_err(|_| CoreError::other("MCP request id space exhausted"))?;
        let mut wire_request = request.clone();
        wire_request.id = wire_id;
        tracing::trace!(
            request_id = request.id,
            wire_id,
            "MCP stdio request correlation"
        );
        let (sender, receiver) = oneshot::channel();
        let pending = match self.dispatcher.register(wire_id, sender) {
            Ok(pending) => pending,
            Err(CoreError::Other(reason)) => return Err(self.disconnect_error(&reason).await),
            Err(error) => return Err(error),
        };
        let mut write_state = MessageWriteState::Queued;
        let dispatch = async {
            self.write_message(&wire_request, &mut write_state).await?;
            match receiver.await {
                Ok(Ok(mut response)) => {
                    response.id = request.id;
                    Ok(response)
                }
                Ok(Err(reason)) => Err(self.disconnect_error(&reason).await),
                Err(_) => Err(CoreError::other(
                    "MCP response dispatcher closed the connection",
                )),
            }
        };
        tokio::select! {
            biased;
            _ = context.cancelled() => {
                drop(pending);
                // A waiter cancelled before acquiring stdin has never issued
                // its request. Do not send an invalid cancellation for that id
                // or misreport its outcome as unknown. An incomplete write has
                // already retired the stream via MessageWrite::drop; only a
                // fully flushed frame can safely be followed by a notification.
                if write_state == MessageWriteState::Queued {
                    return Err(CoreError::other(MCP_CANCELLED_BEFORE_DISPATCH));
                }
                if write_state == MessageWriteState::Flushed {
                    self.notify_cancelled_bounded(wire_id).await;
                }
                Err(CoreError::other(MCP_CANCELLED_UNKNOWN_OUTCOME))
            }
            result = dispatch => result,
        }
    }
}

impl Drop for StdioTransport {
    fn drop(&mut self) {
        self.dispatcher
            .finish("MCP transport closed the connection");
        self.reader_task.abort();
        self.stderr_task.get_mut().abort();
        #[cfg(windows)]
        if let Err(error) = self.process_job.terminate() {
            tracing::error!(%error, "failed to terminate owned MCP job on drop");
        }
        // Dropping the last client during cancellation or config replacement
        // must not orphan its child process.
        let _ = self.child.get_mut().start_kill();
    }
}

#[async_trait]
impl McpTransport for StdioTransport {
    async fn send(&self, request: &JsonRpcRequest) -> Result<JsonRpcResponse> {
        self.send_request(request, ToolExecutionContext::default())
            .await
    }

    async fn send_with_context(
        &self,
        request: &JsonRpcRequest,
        context: ToolExecutionContext,
    ) -> Result<JsonRpcResponse> {
        self.send_request(request, context).await
    }

    async fn notify(&self, method: &str, params: Option<serde_json::Value>) -> Result<()> {
        self.write_notification(method, params).await
    }

    async fn close(&self) -> Result<()> {
        self.dispatcher
            .finish("MCP transport closed the connection");
        self.reader_task.abort();
        #[cfg(windows)]
        {
            self.process_job.shutdown().await.map_err(|error| {
                CoreError::other(format!("wait for owned MCP job shutdown: {error}"))
            })?;
        }
        let mut child = self.child.lock().await;
        let _ = child.start_kill();
        child
            .wait()
            .await
            .map_err(|e| CoreError::other(format!("wait for MCP server shutdown: {e}")))?;
        self.stderr_task.lock().await.abort();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn audit_out_of_order_concurrent_replies_reach_both_callers() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("reordered-replies.cjs");
        std::fs::write(
            &script,
            r#"
const readline = require('node:readline');
const pending = [];
readline.createInterface({ input: process.stdin }).on('line', line => {
  pending.push(JSON.parse(line));
  if (pending.length === 2) {
    for (const request of [...pending].reverse()) {
      process.stdout.write(JSON.stringify({ jsonrpc: '2.0', id: request.id,
        result: { echoed: request.id } }) + '\n');
    }
  }
});
"#,
        )
        .unwrap();
        let config = McpServerConfig {
            transport: None,
            command: Some("node".into()),
            args: vec![script.to_string_lossy().into_owned()],
            env: Default::default(),
            cwd: None,
            url: None,
            headers: Default::default(),
        };
        let transport = Arc::new(StdioTransport::spawn(&config).unwrap());
        let first = tokio::spawn({
            let transport = transport.clone();
            async move {
                tokio::time::timeout(
                    Duration::from_secs(5),
                    transport.send(&JsonRpcRequest::new(10, "ping", None)),
                )
                .await
            }
        });
        // Observe the first registered waiter before issuing the second
        // request. Both responses still have to reach their original callers.
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if !transport
                    .dispatcher
                    .state
                    .lock()
                    .unwrap()
                    .pending
                    .is_empty()
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("first request must register its waiter");

        let second = tokio::time::timeout(
            Duration::from_secs(1),
            transport.send(&JsonRpcRequest::new(11, "ping", None)),
        )
        .await;
        let first = first.await.unwrap();
        transport.close().await.unwrap();
        eprintln!("audit MCP concurrent responses: first={first:?}, second={second:?}");
        assert!(
            matches!(first, Ok(Ok(response)) if response.id == 10)
                && matches!(second, Ok(Ok(response)) if response.id == 11),
            "a response for another pending ID must be delivered, not discarded"
        );
    }

    #[test]
    fn spawn_requires_command() {
        let cfg = McpServerConfig {
            transport: Some(crate::config::TransportType::Stdio),
            command: None,
            args: vec![],
            env: Default::default(),
            cwd: None,
            url: None,
            headers: Default::default(),
        };
        assert!(StdioTransport::spawn(&cfg).is_err());
    }
}

#[cfg(test)]
#[path = "stdio_dispatch_tests.rs"]
mod dispatch_tests;
