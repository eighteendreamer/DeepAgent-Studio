//! The stdio MCP transport: spawn a local server process and speak
//! newline-delimited JSON-RPC 2.0 over its stdin/stdout.
//!
//! This is the most common MCP transport (npx-packaged servers, custom local
//! servers). Each request writes one JSON line to the child's stdin and reads
//! lines from stdout until the response with the matching id arrives (skipping
//! any notifications). The child is terminated on [`McpTransport::close`].

use std::collections::VecDeque;
use std::process::Stdio;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use async_trait::async_trait;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio::sync::Mutex;

use deepagent_core::error::{CoreError, Result};
use deepagent_core::redaction::{is_secret_key, scrub_secret_literals};
use deepagent_tools::ToolExecutionContext;

use crate::config::McpServerConfig;
use crate::protocol::{JsonRpcRequest, JsonRpcResponse};
use crate::transport::{
    McpTransport, MCP_CANCELLED_BEFORE_DISPATCH, MCP_CANCELLED_UNKNOWN_OUTCOME,
};

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;
const STDERR_LIMIT: usize = 8 * 1024;

/// A stdio transport backed by a spawned child process.
pub struct StdioTransport {
    child: Mutex<Child>,
    stdin: Mutex<ChildStdin>,
    stdout: Mutex<BufReader<ChildStdout>>,
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
            .envs(&config.env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if let Some(cwd) = &config.cwd {
            cmd.current_dir(cwd);
        }
        #[cfg(windows)]
        {
            cmd.creation_flags(CREATE_NO_WINDOW);
        }

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

        Ok(Self {
            child: Mutex::new(child),
            stdin: Mutex::new(stdin),
            stdout: Mutex::new(BufReader::new(stdout)),
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
        let line = serde_json::json!({
            "jsonrpc": crate::protocol::JSONRPC_VERSION,
            "method": "notifications/cancelled",
            "params": { "requestId": request_id, "reason": reason }
        });
        let mut wire = serde_json::to_string(&line)?;
        wire.push('\n');
        let mut stdin = self.stdin.lock().await;
        stdin
            .write_all(wire.as_bytes())
            .await
            .map_err(|error| CoreError::other(format!("write MCP cancellation failed: {error}")))?;
        stdin
            .flush()
            .await
            .map_err(|error| CoreError::other(format!("flush MCP cancellation failed: {error}")))
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

    async fn write_request(&self, request: &JsonRpcRequest) -> Result<()> {
        let mut line = serde_json::to_string(request)?;
        line.push('\n');
        let mut stdin = self.stdin.lock().await;
        if let Err(error) = stdin.write_all(line.as_bytes()).await {
            return Err(self
                .disconnect_error(&format!("write to MCP server failed: {error}"))
                .await);
        }
        if let Err(error) = stdin.flush().await {
            return Err(self
                .disconnect_error(&format!("flush to MCP server failed: {error}"))
                .await);
        }
        Ok(())
    }

    async fn read_response(&self, request_id: u64) -> Result<JsonRpcResponse> {
        let mut reader = self.stdout.lock().await;
        loop {
            let mut buf = String::new();
            let n = reader.read_line(&mut buf).await.map_err(|error| {
                CoreError::other(format!("read from MCP server failed: {error}"))
            })?;
            if n == 0 {
                return Err(self
                    .disconnect_error("MCP server closed the connection")
                    .await);
            }
            let trimmed = buf.trim();
            if trimmed.is_empty() {
                continue;
            }
            if let Ok(response) = serde_json::from_str::<JsonRpcResponse>(trimmed) {
                if response.id == request_id {
                    return Ok(response);
                }
            }
        }
    }
}

impl Drop for StdioTransport {
    fn drop(&mut self) {
        self.stderr_task.get_mut().abort();
        // Dropping the last client during cancellation or config replacement
        // must not orphan its child process.
        let _ = self.child.get_mut().start_kill();
    }
}

#[async_trait]
impl McpTransport for StdioTransport {
    async fn send(&self, request: &JsonRpcRequest) -> Result<JsonRpcResponse> {
        self.write_request(request).await?;
        self.read_response(request.id).await
    }

    async fn send_with_context(
        &self,
        request: &JsonRpcRequest,
        context: ToolExecutionContext,
    ) -> Result<JsonRpcResponse> {
        if context.is_cancelled() {
            return Err(CoreError::other(MCP_CANCELLED_BEFORE_DISPATCH));
        }
        let request_id = request.id;
        self.write_request(request).await?;
        if context.is_cancelled() {
            self.notify_cancelled_bounded(request_id).await;
            return Err(CoreError::other(MCP_CANCELLED_UNKNOWN_OUTCOME));
        }

        tokio::select! {
            biased;
            _ = context.cancelled() => {
                self.notify_cancelled_bounded(request_id).await;
                Err(CoreError::other(MCP_CANCELLED_UNKNOWN_OUTCOME))
            }
            result = self.read_response(request_id) => result,
        }
    }

    async fn close(&self) -> Result<()> {
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
    #[ignore = "audit reproducer: concurrent MCP replies must be routed by request ID"]
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
        // Observe the first caller owning the reader before issuing the second
        // request; this fixes the interleaving instead of relying on a sleep.
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if transport.stdout.try_lock().is_err() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("first request must reach the response reader");

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
