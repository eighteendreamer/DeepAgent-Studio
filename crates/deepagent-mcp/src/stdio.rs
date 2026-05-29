//! The stdio MCP transport: spawn a local server process and speak
//! newline-delimited JSON-RPC 2.0 over its stdin/stdout.
//!
//! This is the most common MCP transport (npx-packaged servers, custom local
//! servers). Each request writes one JSON line to the child's stdin and reads
//! lines from stdout until the response with the matching id arrives (skipping
//! any notifications). The child is terminated on [`McpTransport::close`].

use std::process::Stdio;
use std::sync::Mutex;

use async_trait::async_trait;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout};

use deepagent_core::error::{CoreError, Result};

use crate::config::McpServerConfig;
use crate::protocol::{JsonRpcRequest, JsonRpcResponse};
use crate::transport::McpTransport;

/// A stdio transport backed by a spawned child process.
pub struct StdioTransport {
    child: Mutex<Child>,
    stdin: Mutex<ChildStdin>,
    stdout: Mutex<BufReader<ChildStdout>>,
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
            .stderr(Stdio::null()); // MCP servers log to stderr; ignore here.

        let mut child = cmd
            .spawn()
            .map_err(|e| CoreError::other(format!("failed to spawn MCP server '{command}': {e}")))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| CoreError::other("child stdin unavailable"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| CoreError::other("child stdout unavailable"))?;

        Ok(Self {
            child: Mutex::new(child),
            stdin: Mutex::new(stdin),
            stdout: Mutex::new(BufReader::new(stdout)),
        })
    }
}

#[async_trait]
impl McpTransport for StdioTransport {
    async fn send(&self, request: &JsonRpcRequest) -> Result<JsonRpcResponse> {
        // Write the request as a single JSON line.
        let mut line = serde_json::to_string(request)?;
        line.push('\n');
        {
            let mut stdin = self
                .stdin
                .lock()
                .map_err(|_| CoreError::other("stdin mutex poisoned"))?;
            stdin
                .write_all(line.as_bytes())
                .await
                .map_err(|e| CoreError::other(format!("write to MCP server failed: {e}")))?;
            stdin
                .flush()
                .await
                .map_err(|e| CoreError::other(format!("flush to MCP server failed: {e}")))?;
        }

        // Read lines until we find the response with the matching id.
        let mut reader = self
            .stdout
            .lock()
            .map_err(|_| CoreError::other("stdout mutex poisoned"))?;
        loop {
            let mut buf = String::new();
            let n = reader
                .read_line(&mut buf)
                .await
                .map_err(|e| CoreError::other(format!("read from MCP server failed: {e}")))?;
            if n == 0 {
                return Err(CoreError::other("MCP server closed the connection"));
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
        if let Ok(mut child) = self.child.lock() {
            let _ = child.start_kill();
        }
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
            url: None,
            headers: Default::default(),
        };
        assert!(StdioTransport::spawn(&cfg).is_err());
    }
}
