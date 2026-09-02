//! Per-connection runtime state.

use super::config::{SshConnectionConfig, SshStatus};
use async_ssh2_tokio::Client;
use deepagent_terminal::TerminalReadChunk;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex, RwLock};
use tokio::task::JoinHandle;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SshPtyHandle {
    pub connection_id: String,
    pub token: String,
    pub cols: u16,
    pub rows: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SshExecResult {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SshStatusSnapshot {
    pub status: SshStatus,
    pub last_error: Option<String>,
    pub latency_ms: Option<u64>,
}

impl Default for SshStatusSnapshot {
    fn default() -> Self {
        Self {
            status: SshStatus::Disconnected,
            last_error: None,
            latency_ms: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SshTestResult {
    pub ok: bool,
    pub latency_ms: Option<u64>,
    pub banner: Option<String>,
    pub error: Option<String>,
}

pub struct PtyState {
    pub stdin: mpsc::Sender<Vec<u8>>,
    pub stdout: Mutex<mpsc::Receiver<Vec<u8>>>,
    pub join: JoinHandle<()>,
    pub cols: u16,
    pub rows: u16,
    pub output_cursor: AtomicU64,
    pub history: Mutex<VecDeque<(u64, Vec<u8>)>>,
    pub history_bytes: Mutex<usize>,
}

pub struct SshSession {
    config: SshConnectionConfig,
    client: RwLock<Option<Client>>,
    pty: RwLock<Option<PtyState>>,
    status: RwLock<SshStatus>,
    last_error: RwLock<Option<String>>,
    keepalive_running: AtomicBool,
    last_keepalive_ms: AtomicU64,
}

impl SshSession {
    pub fn new(config: SshConnectionConfig) -> Arc<Self> {
        Arc::new(Self {
            config,
            client: RwLock::new(None),
            pty: RwLock::new(None),
            status: RwLock::new(SshStatus::Disconnected),
            last_error: RwLock::new(None),
            keepalive_running: AtomicBool::new(false),
            last_keepalive_ms: AtomicU64::new(0),
        })
    }

    pub fn key(&self) -> &str {
        &self.config.id
    }

    pub async fn client(&self) -> Option<Client> {
        self.client.read().await.clone()
    }

    pub async fn set_client(&self, client: Option<Client>) {
        *self.client.write().await = client;
    }

    pub async fn status(&self) -> SshStatus {
        *self.status.read().await
    }

    pub async fn last_error(&self) -> Option<String> {
        self.last_error.read().await.clone()
    }

    pub async fn set_status(&self, status: SshStatus, error: Option<String>) {
        *self.status.write().await = status;
        *self.last_error.write().await = error;
    }

    pub fn touch_keepalive(&self) {
        self.last_keepalive_ms.store(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0),
            Ordering::Relaxed,
        );
    }

    pub fn is_keepalive_running(&self) -> bool {
        self.keepalive_running.load(Ordering::Relaxed)
    }

    pub fn set_keepalive_running(&self, running: bool) {
        self.keepalive_running.store(running, Ordering::Relaxed);
    }

    pub async fn replace_pty(&self, next: Option<PtyState>) {
        let mut guard = self.pty.write().await;
        if let Some(old) = std::mem::replace(&mut *guard, next) {
            old.join.abort();
        }
    }

    pub async fn pty_write(&self, data: Vec<u8>) -> bool {
        let pty = self.pty.read().await;
        let Some(pty) = pty.as_ref() else {
            return false;
        };
        pty.stdin.send(data).await.is_ok()
    }

    pub async fn pty_read_available(&self) -> Option<Vec<u8>> {
        let pty = self.pty.read().await;
        let pty = pty.as_ref()?;
        let mut rx = pty.stdout.lock().await;
        let mut out = Vec::new();
        while let Ok(chunk) = rx.try_recv() {
            out.extend_from_slice(&chunk);
            if out.len() >= 64 * 1024 {
                break;
            }
        }
        Some(out)
    }

    pub async fn pty_read_with_cursor(&self, after_cursor: u64) -> Option<TerminalReadChunk> {
        let pty = self.pty.read().await;
        let pty = pty.as_ref()?;
        let mut rx = pty.stdout.lock().await;
        let mut history = pty.history.lock().await;
        let mut history_bytes = pty.history_bytes.lock().await;
        while let Ok(chunk) = rx.try_recv() {
            if chunk.is_empty() {
                continue;
            }
            let start = pty
                .output_cursor
                .fetch_add(chunk.len() as u64, Ordering::AcqRel);
            *history_bytes += chunk.len();
            history.push_back((start, chunk));
            while *history_bytes > 256 * 1024 {
                if let Some((_, old)) = history.pop_front() {
                    *history_bytes = history_bytes.saturating_sub(old.len());
                } else {
                    break;
                }
            }
        }
        let current = pty.output_cursor.load(Ordering::Acquire);
        let oldest = history.front().map(|(start, _)| *start).unwrap_or(current);
        let truncated = after_cursor < oldest && oldest > 0;
        let mut data = Vec::new();
        for (start, chunk) in history.iter() {
            let end = *start + chunk.len() as u64;
            if end > after_cursor {
                let offset = after_cursor.saturating_sub(*start) as usize;
                data.extend_from_slice(&chunk[offset.min(chunk.len())..]);
                if data.len() >= 64 * 1024 {
                    data.truncate(64 * 1024);
                    break;
                }
            }
        }
        Some(TerminalReadChunk {
            cursor: current,
            data,
            truncated,
        })
    }

    pub async fn pty_resize(&self, cols: u16, rows: u16) -> bool {
        let mut pty = self.pty.write().await;
        let Some(pty) = pty.as_mut() else {
            return false;
        };
        pty.cols = cols;
        pty.rows = rows;
        true
    }

    pub fn config(&self) -> &SshConnectionConfig {
        &self.config
    }
}
