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

use crate::config::McpServerConfig;
use crate::protocol::{JsonRpcRequest, JsonRpcResponse};
use crate::transport::McpTransport;

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
        // Write the request as a single JSON line.
        let mut line = serde_json::to_string(request)?;
        line.push('\n');
        {
            let mut stdin = self.stdin.lock().await;
            let written = stdin.write_all(line.as_bytes()).await;
            if let Err(error) = written {
                return Err(self
                    .disconnect_error(&format!("write to MCP server failed: {error}"))
                    .await);
            }
            if let Err(error) = stdin.flush().await {
                return Err(self
                    .disconnect_error(&format!("flush to MCP server failed: {error}"))
                    .await);
            }
        }

        // Read lines until we find the response with the matching id.
        let mut reader = self.stdout.lock().await;
        loop {
            let mut buf = String::new();
            let n = reader
                .read_line(&mut buf)
                .await
                .map_err(|e| CoreError::other(format!("read from MCP server failed: {e}")))?;
            if n == 0 {
                return Err(self
                    .disconnect_error("MCP server closed the connection")
                    .await);
            }
            let trimmed = buf.trim();
            if trimmed.is_empty() {
                continue;
            }
            // Try to parse as a response; skip lines that are notifications
            // (no id) or unrelated.
            if let Ok(resp) = serde_json::from_str::<JsonRpcResponse>(trimmed) {
                if resp.id == request.id {
                    return Ok(resp);
                }
            }
            // Otherwise keep reading (notification / log line).
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
